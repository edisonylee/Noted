import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { ArrowLeft, BookOpen, Check, ChevronRight, Copy, Folder, Loader, Lock, LogOut, Plus, RefreshCw, Search, Settings, Trash2, Users } from "lucide-react";
import { MdBlock } from "../MeetingMarkdownView";
import { team, orgPath, copyTeamText } from "./client";
import type { TeamOrg, TeamSnapshot, TeamSpace, TeamFolder, TeamNote, TeamNoteRow, TeamAnswer } from "./types";
import { TeamDialog } from "./TeamDialog";
import { SavedAnswers } from "./SavedAnswers";
import { TeamAdministration } from "./TeamAdministration";
import "./teams.css";

export function TeamConnect({ onConnected }: { onConnected: (orgs: TeamOrg[]) => void }) {
  const [server, setServer] = useState("");
  const [mode, setMode] = useState("join");
  const [secret, setSecret] = useState("");
  const [name, setName] = useState("");
  const [organization, setOrganization] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => { team.status().then(s => setServer(s.server)).catch(() => {}); }, []);
  const submit = async (e: FormEvent) => {
    e.preventDefault(); if (busy) return; setBusy(true); setError("");
    try { const orgs = await team.connect(server, mode, secret, organization, name); setSecret(""); onConnected(orgs); }
    catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  return <section className="team-connect">
    <h1>A shared memory for your team.</h1>
    <p>Bring selected meetings into one place. Find decisions, follow the context, and give the right people access.</p>
    <form onSubmit={submit} className="team-form">
      <label>Team server<input type="url" value={server} onChange={e => setServer(e.target.value)} placeholder="https://notes.yourcompany.com" required autoComplete="url" /></label>
      <label>Connection<select value={mode} onChange={e => { setMode(e.target.value); setSecret(""); }}>
        <option value="join">Join with an invitation</option><option value="signin">Sign in with an access key</option><option value="create">Set up a new team server</option>
      </select></label>
      {mode === "create" && <><label>Workspace name<input value={organization} onChange={e => setOrganization(e.target.value)} maxLength={200} required /></label><label>Your name<input value={name} onChange={e => setName(e.target.value)} maxLength={200} required autoComplete="name" /></label></>}
      <label>{mode === "join" ? "Invitation code" : mode === "create" ? "Server setup key" : "Access key"}<input type="password" value={secret} onChange={e => setSecret(e.target.value)} required autoComplete="off" spellCheck={false} /></label>
      {error && <p className="team-error" role="alert">{error}</p>}
      <button className="team-primary" disabled={busy}>{busy ? <Loader size={15} className="spin" /> : null}{mode === "create" ? "Create workspace" : "Connect to team"}</button>
    </form>
    <p className="team-privacy"><Lock size={14} /> Your local library stays private. You choose which meetings to publish.</p>
  </section>;
}
export function TeamWorkspace() {
  const [orgs, setOrgs] = useState<TeamOrg[] | null>(null);
  const [org, setOrg] = useState("");
  const [connected, setConnected] = useState<boolean | null>(null);
  const [error, setError] = useState("");
  const [addWorkspace, setAddWorkspace] = useState(false);
  useEffect(() => { let active = true;
    team.status().then(async status => {
      if (!active) return; setConnected(status.connected);
      if (status.connected) { const values = await team.request<TeamOrg[]>("GET", "/v1/orgs"); if (active) { setOrgs(values); setOrg(values[0]?.id ?? ""); } }
    }).catch(e => { if (active) { setError(String(e)); setConnected(false); } });
    return () => { active = false; };
  }, []);
  if (connected == null) return <div className="team-loading"><Loader className="spin" size={18} /> Opening team workspace…</div>;
  if (!connected) return <><TeamConnect onConnected={values => { setOrgs(values); setOrg(values[0]?.id ?? ""); setConnected(true); setError(""); }} />{error && <p className="team-error">{error}</p>}</>;
  return <div className="team-workspace">
    <div className="team-workspace-bar">
      <label><span className="sr-only">Team workspace</span><select value={org} onChange={e => setOrg(e.target.value)}>{(orgs ?? []).map(o => <option key={o.id} value={o.id}>{o.name}</option>)}</select></label>
      <span>Shared workspace</span>
      <button className="team-text-button" onClick={() => setAddWorkspace(true)}><Plus size={14} /> Workspace</button>
      <button className="team-text-button" onClick={async () => { try { await team.disconnect(); setConnected(false); setOrgs(null); setOrg(""); } catch (e) { setError(String(e)); } }}><LogOut size={14} /> Sign out</button>
    </div>
    {error && <p className="team-error" role="alert">{error}</p>}
    {org ? <TeamLibrary key={org} org={org} /> : <p className="team-empty">You don’t have access to a workspace. Ask an admin for a new invitation.</p>}
    {addWorkspace && <AddWorkspace onClose={() => setAddWorkspace(false)} onAdded={async id => { const values = await team.request<TeamOrg[]>("GET", "/v1/orgs"); setOrgs(values); setOrg(id); setAddWorkspace(false); }} />}
  </div>;
}

function AddWorkspace({ onClose, onAdded }: { onClose: () => void; onAdded: (id: string) => Promise<void> }) {
  const [mode, setMode] = useState("join"), [value, setValue] = useState("");
  const [error, setError] = useState(""), [busy, setBusy] = useState(false);
  return <TeamDialog title="Add a workspace" onClose={onClose}><form className="team-form" onSubmit={async e => {
    e.preventDefault(); if (busy) return; setBusy(true); setError("");
    try { const result = await team.request<{ id?: string; org?: string }>("POST", mode === "join" ? "/v1/orgs/join" : "/v1/orgs", mode === "join" ? { invitation: value } : { name: value }); await onAdded(result.org ?? result.id!); }
    catch (e) { setError(String(e)); } finally { setBusy(false); }
  }}><label>Workspace action<select value={mode} onChange={e => { setMode(e.target.value); setValue(""); }}><option value="join">Join an existing workspace</option><option value="create">Create a workspace</option></select></label><label>{mode === "join" ? "Invitation code" : "Workspace name"}<input autoFocus required maxLength={200} type={mode === "join" ? "password" : "text"} value={value} onChange={e => setValue(e.target.value)} /></label><p className="team-muted">Workspaces on your current team server have separate members, spaces, and shared content.</p>{error && <p className="team-error" role="alert">{error}</p>}<button className="team-primary" disabled={busy}>{mode === "join" ? "Join workspace" : "Create workspace"}</button></form></TeamDialog>;
}

function TeamLibrary({ org }: { org: string }) {
  const [data, setData] = useState<TeamSnapshot | null>(null);
  const [space, setSpace] = useState("");
  const [folder, setFolder] = useState("");
  const [view, setView] = useState<"notes" | "admin" | "trash" | "answers">("notes");
  const [query, setQuery] = useState("");
  const [rows, setRows] = useState<TeamNoteRow[]>([]);
  const [more, setMore] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [noteId, setNoteId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [question, setQuestion] = useState("");
  const [answer, setAnswer] = useState<TeamAnswer | null>(null);
  const [asking, setAsking] = useState(false);
  const [answerQuestion, setAnswerQuestion] = useState(""), [saved, setSaved] = useState(false), [saving, setSaving] = useState(false);
  const [editor, setEditor] = useState<"space" | "folder" | "editFolder" | null>(null);
  const [accessEpoch, setAccessEpoch] = useState(0);
  const accessVersion = useRef<number | null>(null);
  const requestVersion = useRef(0), scopeVersion = useRef(0);
  const refresh = useCallback(async () => {
    const next = await team.request<TeamSnapshot>("GET", orgPath(org));
    if (accessVersion.current != null && accessVersion.current !== next.access_version) {
      ++requestVersion.current; ++scopeVersion.current;
      setRows([]); setSelected([]); setAnswer(null); setAsking(false); setNoteId(null);
      setAccessEpoch(v => v + 1);
    }
    accessVersion.current = next.access_version;
    setData(next); return next;
  }, [org]);
  const loadRows = useCallback(async (offset = 0) => {
    const version = ++requestVersion.current; setLoading(true);
    try {
      const params = new URLSearchParams({ q: query, space, folder, trash: String(view === "trash"), offset: String(offset) });
      const next = await team.request<TeamNoteRow[]>("GET", orgPath(org, `/notes?${params}`));
      if (requestVersion.current === version) { setRows(old => offset ? [...old, ...next] : next); setMore(next.length === 100); setError(""); }
    } catch (e) { if (requestVersion.current === version) { setRows([]); setAnswer(null); setError(String(e)); } }
    finally { if (requestVersion.current === version) setLoading(false); }
  }, [org, query, space, folder, view]);
  useEffect(() => { refresh().catch(e => setError(String(e))); }, [refresh]);
  useEffect(() => {
    ++scopeVersion.current; ++requestVersion.current;
    setRows([]); setSelected([]); setAnswer(null); setNoteId(null); setAsking(false);
    const timer = window.setTimeout(() => { if (view === "notes" || view === "trash") void loadRows(); }, 180);
    return () => { clearTimeout(timer); ++requestVersion.current; ++scopeVersion.current; };
  }, [loadRows, view, accessEpoch]);
  // Recheck access while the workspace is visible, including after a Mac wakes.
  useEffect(() => {
    const check = () => { if (document.visibilityState === "visible") refresh().then(next => {
      if (space && !next.spaces.some(s => s.id === space)) { setSpace(""); setFolder(""); setNoteId(null); setAnswer(null); }
    }).catch(e => { ++scopeVersion.current; ++requestVersion.current; setData(null); setRows([]); setSelected([]); setAsking(false); setNoteId(null); setAnswer(null); setError(String(e)); }); };
    const timer = window.setInterval(check, 30_000); window.addEventListener("focus", check);
    return () => { clearInterval(timer); window.removeEventListener("focus", check); };
  }, [refresh, space]);
  const ask = async (e?: FormEvent) => {
    e?.preventDefault(); if (!question.trim() || asking) return;
    const version = scopeVersion.current; setAsking(true); setAnswer(null); setSaved(false); setError("");
    try { const result = await team.ask(org, { question, space_id: space, folder_id: folder, note_ids: selected }); if (version === scopeVersion.current) { setAnswer(result); setAnswerQuestion(question); } }
    catch (e) { if (version === scopeVersion.current) setError(String(e)); }
    finally { if (version === scopeVersion.current) setAsking(false); }
  };
  const navigate = (nextSpace = "", nextFolder = "") => { setSpace(nextSpace); setFolder(nextFolder); setView("notes"); setQuery(""); setNoteId(null); };
  const changeSelection = (ids: string[]) => { ++scopeVersion.current; setSelected(ids); setAnswer(null); setAsking(false); };
  const currentSpace = data?.spaces.find(s => s.id === space);
  const currentFolder = data?.folders.find(f => f.id === folder);
  const scopeName = currentFolder?.name ?? currentSpace?.name ?? "All shared meetings";
  const isAdmin = data?.org.role === "owner" || data?.org.role === "admin";
  const nestedFolders = (spaceId: string, parent: string | null = null, depth = 0): ReactNode => data?.folders.filter(f => f.space_id === spaceId && f.parent_id === parent).map(f => <div key={f.id}>
    <button style={{ paddingLeft: `${20 + Math.min(depth, 5) * 12}px` }} className={folder === f.id ? "on" : ""} onClick={() => navigate(spaceId, f.id)}><Folder size={14} /><span>{f.name}</span></button>
    {depth < 20 && nestedFolders(spaceId, f.id, depth + 1)}
  </div>);
  return <div className="team-layout">
    <aside className="team-sidebar" aria-label="Team library">
      <button className={view === "notes" && !space ? "on" : ""} onClick={() => navigate()}><BookOpen size={15} /> All shared meetings</button>
      <div className="team-sidebar-label"><span>Spaces</span>{isAdmin && <button aria-label="Create team space" onClick={() => setEditor("space")}><Plus size={15} /></button>}</div>
      {data?.spaces.map(s => <div key={s.id} className="team-space-nav">
        <button className={space === s.id && !folder ? "on" : ""} onClick={() => navigate(s.id)}>{s.visibility === "restricted" ? <Lock size={14} /> : <Users size={14} />}<span>{s.name}</span></button>
        {nestedFolders(s.id)}
      </div>)}
      <div className="team-sidebar-bottom">
        <button className={view === "answers" ? "on" : ""} onClick={() => { setView("answers"); setNoteId(null); }}><BookOpen size={15} /> Saved answers</button>
        <button className={view === "admin" ? "on" : ""} onClick={() => { setView("admin"); setNoteId(null); }}><Settings size={15} /> Members & prompts</button>
        <button className={view === "trash" ? "on" : ""} onClick={() => { setView("trash"); setSpace(""); setFolder(""); }}><Trash2 size={15} /> Trash</button>
      </div>
    </aside>
    <main className="team-main">
      {error && <div className="team-error" role="alert">{error}<button className="team-text-button" onClick={() => { void refresh().then(() => loadRows()).catch(e => setError(String(e))); }}>Retry</button></div>}
      {!data && !error && <div className="team-loading"><Loader size={16} className="spin" /> Loading workspace…</div>}
      {data && view === "answers" && !noteId ? <SavedAnswers key={accessEpoch} org={org} onSource={id => setNoteId(id)} /> : data && view === "admin" ? <TeamAdministration data={data} refresh={refresh} /> : data && noteId ? <SharedMeeting key={noteId} org={org} id={noteId} folders={data.folders} onBack={() => { setNoteId(null); void loadRows(); }} /> : data && <>
        <header className="team-library-head"><div><h1>{view === "trash" ? "Trash" : scopeName}</h1><p>{view === "trash" ? "Removed shared copies. Local originals remain in their owners’ libraries." : currentFolder?.description || currentSpace?.description || "The conversations your team chose to bring together."}</p></div>
          <button className="team-text-button" aria-label="Refresh shared meetings" onClick={() => { void refresh().then(() => loadRows()).catch(e => setError(String(e))); }}><RefreshCw size={15} /></button>
          {currentSpace?.role === "editor" && view === "notes" && <button className="team-text-button" onClick={() => setEditor("folder")}><Plus size={14} /> Folder</button>}
          {currentFolder && currentSpace?.role === "editor" && view === "notes" && <button className="team-text-button" onClick={() => setEditor("editFolder")}>Edit folder</button>}
        </header>
        {view === "notes" && <section className="team-ask" aria-label="Ask shared meetings">
          <form onSubmit={ask}><label className="sr-only" htmlFor="team-question">Ask shared meetings</label><textarea id="team-question" rows={2} placeholder={`What would you like to know about ${currentFolder?.name ?? currentSpace?.name ?? "your team’s meetings"}?`} value={question} maxLength={6000} onChange={e => setQuestion(e.target.value)} onKeyDown={e => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); void ask(); } }} />
            <div className="team-ask-footer"><span>{selected.length ? `${selected.length} selected ${selected.length === 1 ? "meeting" : "meetings"}` : scopeName}</span><button className="team-primary" disabled={asking || !question.trim() || rows.length === 0}>{asking ? <Loader size={15} className="spin" /> : null}{asking ? "Reading sources…" : "Ask"}</button></div>
          </form>
          {!!data.recipes.filter(r => r.kind === "recipe").length && <div className="team-recipe-shortcuts">{data.recipes.filter(r => r.kind === "recipe").slice(0, 4).map(r => <button key={r.id} onClick={() => setQuestion(r.prompt)}>{r.name}<ChevronRight size={12} /></button>)}</div>}
          {answer && <div className="team-answer"><MdBlock md={answer.answer} /><button className="team-text-button" disabled={saved || saving || !answer.sources.length} onClick={async () => { if (saving) return; setSaving(true); try { await team.request("POST", orgPath(org, "/answers"), { ...answer, question: answerQuestion }); setSaved(true); } catch (e) { setError(String(e)); } finally { setSaving(false); } }}>{saved ? "Saved to your answers" : saving ? "Saving…" : "Save answer"}</button><div className="team-sources">{answer.sources.map(s => <button key={s.id} onClick={() => setNoteId(s.id)}><span>[{s.citation}]</span>{s.title}</button>)}</div>{answer.limited && <p className="team-muted">This answer uses a limited selection of excerpts. Narrow the folder or select meetings for a closer review.</p>}</div>}
        </section>}
        <div className="team-list-tools"><label><Search size={15} /><input type="search" aria-label="Search shared notes and transcripts" placeholder="Search notes and transcripts" value={query} maxLength={500} onChange={e => setQuery(e.target.value)} /></label><span>{selected.length ? `${selected.length} selected` : `${rows.length}${more ? "+" : ""} ${rows.length === 1 ? "meeting" : "meetings"}`}</span></div>
        {selected.length > 0 && <div className="team-selection"><button onClick={() => changeSelection([])}>Clear selection</button><span>Ask a question above to work with these meetings (up to 40).</span></div>}
        <div className="team-note-list" aria-busy={loading}>
          {rows.map(row => <div className="team-note-row" key={row.id}>
            {view === "notes" && <input type="checkbox" checked={selected.includes(row.id)} disabled={selected.length >= 40 && !selected.includes(row.id)} aria-label={`Select ${row.title}`} onChange={e => changeSelection(e.target.checked ? [...selected, row.id] : selected.filter(id => id !== row.id))} />}
            <button onClick={() => setNoteId(row.id)}><strong>{row.title}</strong><span className="team-excerpt">{row.excerpt.replace(/^#+\s*/gm, "").replace(/\*\*/g, "")}</span><span className="team-note-meta">{row.owner_name} · {new Date(row.occurred_at).toLocaleDateString(undefined, { month: "short", day: "numeric" })}{row.has_transcript ? " · Transcript included" : ""}</span></button>
            <ChevronRight size={15} aria-hidden="true" />
          </div>)}
          {loading && <div className="team-loading"><Loader size={16} className="spin" /> Loading meetings…</div>}
          {!loading && !rows.length && <div className="team-empty"><h2>{query ? "No shared meetings match." : view === "trash" ? "Nothing in Trash." : "Start with a conversation worth sharing."}</h2><p>{query ? "Try a name, decision, or phrase from the transcript." : view === "trash" ? "Removed shared meetings will appear here." : "Open a completed meeting in your Library and choose Share → Publish to team. Only the content you review is published."}</p></div>}
          {more && !loading && <button className="team-text-button" onClick={() => void loadRows(rows.length)}>Load more meetings</button>}
        </div>
      </>}
      {editor && data && <CreateTeamLocation kind={editor} org={org} space={currentSpace} parent={currentFolder} folders={data.folders} onClose={() => setEditor(null)} onSaved={async () => { setEditor(null); await refresh(); }} />}
    </main>
  </div>;
}

function CreateTeamLocation({ kind, org, space, parent, folders, onClose, onSaved }: { kind: "space" | "folder" | "editFolder"; org: string; space?: TeamSpace; parent?: TeamFolder; folders: TeamFolder[]; onClose: () => void; onSaved: () => Promise<void> }) {
  const [name, setName] = useState(kind === "editFolder" ? parent?.name ?? "" : ""), [description, setDescription] = useState(kind === "editFolder" ? parent?.description ?? "" : ""), [visibility, setVisibility] = useState("restricted");
  const [parentId, setParentId] = useState(kind === "editFolder" ? parent?.parent_id ?? "" : parent?.id ?? "");
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  return <TeamDialog title={kind === "space" ? "Create a team space" : kind === "editFolder" ? "Edit folder" : "Create a folder"} onClose={onClose}>
    <form className="team-form" onSubmit={async e => { e.preventDefault(); if (busy) return; setBusy(true); setError(""); try { await team.request(kind === "editFolder" ? "PUT" : "POST", orgPath(org, kind === "space" ? "/spaces" : kind === "editFolder" ? `/folders/${parent?.id}` : "/folders"), { name, description, visibility, space_id: space?.id, parent_id: parentId || null }); await onSaved(); } catch (e) { setError(String(e)); } finally { setBusy(false); } }}>
      <label>Name<input autoFocus required value={name} maxLength={200} onChange={e => setName(e.target.value)} /></label>
      <label>Description<textarea value={description} maxLength={4000} onChange={e => setDescription(e.target.value)} rows={3} /></label>
      {kind === "space" && <label>Who can access<select value={visibility} onChange={e => setVisibility(e.target.value)}><option value="restricted">Admins and invited members or groups</option><option value="team">Everyone in this workspace</option></select></label>}
      {kind !== "space" && <><label>Parent folder<select value={parentId} onChange={e => setParentId(e.target.value)}><option value="">{space?.name} (top level)</option>{folders.filter(f => f.space_id === space?.id && (kind !== "editFolder" || f.id !== parent?.id)).map(f => <option key={f.id} value={f.id}>{f.name}</option>)}</select></label><p className="team-muted">Folders inherit their space’s access.</p></>}
      {error && <p className="team-error" role="alert">{error}</p>}<button className="team-primary" disabled={busy}>{kind === "editFolder" ? "Save folder" : `Create ${kind}`}</button>
    </form>
  </TeamDialog>;
}


function SharedMeeting({ org, id, folders, onBack }: { org: string; id: string; folders: TeamFolder[]; onBack: () => void }) {
  const [note, setNote] = useState<TeamNote | null>(null), [error, setError] = useState("");
  const [editing, setEditing] = useState(false), [title, setTitle] = useState(""), [summary, setSummary] = useState("");
  const [busy, setBusy] = useState(false), [copied, setCopied] = useState(false);
  const [editRevision, setEditRevision] = useState<number | null>(null);
  const [editFolders, setEditFolders] = useState<string[]>([]);
  const transcriptPanel = useRef<HTMLDetailsElement>(null);
  const transcriptRows = useRef<(HTMLParagraphElement | null)[]>([]);
  const [highlight, setHighlight] = useState(-1);
  const openSource = (source: string) => {
    if (!note?.transcript || source.toLowerCase() === "notes") { setError("This source was not included in the published meeting."); return; }
    const seconds = (value: string) => { const match = value.match(/(\d+):(\d{2})/); return match ? Number(match[1]) * 60 + Number(match[2]) : -1; };
    const target = seconds(source), lines = note.transcript.split("\n");
    let closest = -1, distance = Infinity;
    lines.forEach((line, i) => { const time = seconds(line); if (time >= 0 && Math.abs(time - target) < distance) { closest = i; distance = Math.abs(time - target); } });
    if (closest < 0) { setError("No timestamped transcript passage is available."); return; }
    if (transcriptPanel.current) transcriptPanel.current.open = true;
    setHighlight(closest); setError("");
    requestAnimationFrame(() => transcriptRows.current[closest]?.scrollIntoView({ block: "center" }));
  };
  const load = useCallback(async () => { const n = await team.request<TeamNote>("GET", orgPath(org, `/notes/${id}`)); setNote(n); return n; }, [org, id]);
  useEffect(() => { let active = true; team.request<TeamNote>("GET", orgPath(org, `/notes/${id}`)).then(n => { if (active) setNote(n); }).catch(e => { if (active) setError(String(e)); }); return () => { active = false; }; }, [org, id]);
  useEffect(() => { const timer = window.setInterval(() => { load().catch(e => { setNote(null); setError(String(e)); }); }, 30_000); return () => clearInterval(timer); }, [load]);
  return <article className="team-shared-note"><button className="team-text-button" onClick={() => { if (!editing || confirm("Discard unsaved changes to this shared meeting?")) onBack(); }}><ArrowLeft size={15} /> Shared meetings</button>
    {error && <p className="team-error" role="alert">{error}<button className="team-text-button" onClick={() => void load().then(() => setError("")).catch(e => setError(String(e)))}>Reload</button></p>}
    {!note && !error && <p>Loading meeting…</p>}
    {note && <><header><h1>{note.title}</h1><p className="team-muted">Shared by {note.owner_name} · {new Date(note.occurred_at).toLocaleDateString()} · Revision {note.revision}</p></header>
      <div className="team-note-actions"><button onClick={async () => { try { await copyTeamText(`# ${note.title}\n\n${note.summary}${note.transcript ? `\n\n## Transcript\n${note.transcript}` : ""}`); setCopied(true); } catch (e) { setError(String(e)); } }}>{copied ? <Check size={14} /> : <Copy size={14} />}{copied ? "Copied" : "Copy Markdown"}</button>
        {note.can_edit && !note.trashed_at && !editing && <button onClick={() => { setTitle(note.title); setSummary(note.summary); setEditRevision(note.revision); setEditFolders(note.folder_ids); setEditing(true); }}>Edit shared notes</button>}
        {note.can_manage && <button disabled={busy} onClick={async () => { if (!note.trashed_at && !confirm("Move this shared copy to team Trash? Your local meeting is kept.")) return; setBusy(true); try { await team.request(note.trashed_at ? "POST" : "DELETE", orgPath(org, `/notes/${id}${note.trashed_at ? "/restore" : ""}`), { revision: note.revision }); onBack(); } catch (e) { setError(String(e)); } finally { setBusy(false); } }}>{note.trashed_at ? "Restore" : "Move to Trash"}</button>}
      </div>
      {editing ? <TeamDialog title="Edit shared meeting" busy={busy} onClose={() => { if (confirm("Discard unsaved changes to this shared meeting?")) setEditing(false); }}>{error && <p className="team-error" role="alert">{error}</p>}<form className="team-form" onSubmit={async e => { e.preventDefault(); if (busy) return; setBusy(true); try { const n = await team.request<TeamNote>("PATCH", orgPath(org, `/notes/${id}`), { title, summary, folder_ids: editFolders, revision: editRevision }); setNote(n); setEditing(false); setError(""); } catch (e) { setError(String(e)); } finally { setBusy(false); } }}><label>Title<input value={title} required maxLength={500} onChange={e => setTitle(e.target.value)} /></label><label>Shared notes<textarea value={summary} required rows={18} maxLength={300000} onChange={e => setSummary(e.target.value)} /></label><fieldset><legend>Folders</legend>{folders.filter(f => f.space_id === note.space_id).map(f => <label key={f.id} className="team-checkbox"><input type="checkbox" checked={editFolders.includes(f.id)} onChange={e => setEditFolders(old => e.target.checked ? [...old, f.id] : old.filter(id => id !== f.id))} />{f.name}</label>)}</fieldset><div className="team-form-actions"><button className="team-primary" disabled={busy}>Save shared notes</button><button type="button" className="team-text-button" onClick={() => setEditing(false)}>Cancel</button></div></form></TeamDialog> : <MdBlock md={note.summary} onSource={openSource} />}
      {!!note.transcript && <details ref={transcriptPanel} className="team-transcript"><summary>Full transcript</summary><div className="team-transcript-lines">{note.transcript.split("\n").map((line, i) => <p key={i} ref={node => { transcriptRows.current[i] = node; }} className={highlight === i ? "team-source-highlight" : ""}>{line}</p>)}</div></details>}
      <p className="team-muted team-note-provenance">This is a shared copy. Changes here do not overwrite the publisher’s local meeting.</p>
    </>}
  </article>;
}
