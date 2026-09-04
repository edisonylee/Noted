import { useEffect, useState } from "react";
import { Check, Lock, Users } from "lucide-react";
import { api, type MeetingDetail } from "../api";
import { MdBlock } from "../MeetingMarkdownView";
import { team, orgPath } from "./client";
import { TeamDialog } from "./TeamDialog";
import type { TeamOrg, TeamSnapshot } from "./types";
import "./teams.css";

export function PublishMeeting({ meeting, summaryId, onClose }: { meeting: MeetingDetail; summaryId: number | null; onClose: () => void }) {
  const [orgs, setOrgs] = useState<TeamOrg[]>([]), [org, setOrg] = useState("");
  const [data, setData] = useState<TeamSnapshot | null>(null), [space, setSpace] = useState("");
  const [folders, setFolders] = useState<string[]>([]), [transcript, setTranscript] = useState(false);
  const [chosenSummary, setChosenSummary] = useState(summaryId ?? meeting.summaries[0]?.id ?? null);
  const [error, setError] = useState(""), [busy, setBusy] = useState(false), [done, setDone] = useState(false);
  useEffect(() => { let active = true; team.request<TeamOrg[]>("GET", "/v1/orgs").then(values => { if (active) { setOrgs(values); setOrg(values[0]?.id ?? ""); } }).catch(() => { if (active) setError("Connect to a workspace from Team in the sidebar before publishing a meeting."); }); return () => { active = false; }; }, []);
  useEffect(() => { let active = true; setData(null); setSpace(""); setFolders([]); if (org) team.request<TeamSnapshot>("GET", orgPath(org)).then(value => { if (active) setData(value); }).catch(e => { if (active) setError(String(e)); }); return () => { active = false; }; }, [org]);
  const destination = data?.spaces.find(s => s.id === space), summary = meeting.summaries.find(s => s.id === chosenSummary);
  const reviewedTranscript = transcript ? meeting.segments.map(s => {
    const seconds = Math.floor(Math.max(0, s.t0_ms) / 1000);
    const time = `${String(Math.floor(seconds / 60)).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;
    return `[${time}] ${s.speaker || (s.channel === "me" ? "Me" : "Them")}: ${s.text}`;
  }).join("\n") : "";
  const publish = async () => {
    if (!org || !space || !summary || busy) return; setBusy(true); setError("");
    try {
      // A random per-vault prefix keeps local row IDs from identifying another
      // person's meeting, and remains stable across retries on this Mac.
      let installation = localStorage.getItem("noted-team-publication-origin");
      if (!installation) { installation = crypto.randomUUID(); localStorage.setItem("noted-team-publication-origin", installation); }
      await api.teamPublishMeeting({ org, id: meeting.id, spaceId: space, folderIds: folders, summaryId: summary.id, includeTranscript: transcript, sourceKey: `${installation}:${meeting.id}`, reviewedContent: { title: meeting.title, summary: summary.content_md, transcript: reviewedTranscript, accessVersion: data!.access_version } });
      setDone(true);
    } catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  return <TeamDialog title={done ? "Meeting shared" : "Publish a meeting to your team"} onClose={onClose} busy={busy}>
    {done ? <div className="team-published"><Check size={22} /><h3>{meeting.title}</h3><p>Published to {data?.org.name} / {destination?.name}. Your team can now find it in the shared workspace.</p><button className="team-primary" onClick={onClose}>Done</button></div> : <div className="team-form">
      <p>Share a copy of <strong>{meeting.title}</strong>. Review the summary before publishing.</p>
      <label>Workspace<select value={org} onChange={e => setOrg(e.target.value)} disabled={busy}><option value="">Choose a workspace</option>{orgs.map(o => <option key={o.id} value={o.id}>{o.name}</option>)}</select></label>
      <label>Destination space<select value={space} onChange={e => { setSpace(e.target.value); setFolders([]); }} disabled={busy}><option value="">Choose who gets access</option>{data?.spaces.filter(s => s.role === "editor").map(s => <option key={s.id} value={s.id}>{s.name} · {s.visibility === "team" ? "Whole workspace" : "Restricted"}</option>)}</select></label>
      {destination && <p className="team-audience">{destination.visibility === "restricted" ? <Lock size={15} /> : <Users size={15} />}{destination.visibility === "team" ? `Everyone in ${data?.org.name} can read this copy.` : "Workspace admins and members or groups with access to this space can read this copy."}</p>}
      {!!data?.folders.filter(f => f.space_id === space).length && <fieldset><legend>Folders (optional)</legend>{data?.folders.filter(f => f.space_id === space).map(f => <label key={f.id} className="team-checkbox"><input type="checkbox" checked={folders.includes(f.id)} disabled={busy} onChange={e => setFolders(old => e.target.checked ? [...old, f.id] : old.filter(id => id !== f.id))} />{f.name}</label>)}</fieldset>}
      <label>Summary<select value={chosenSummary ?? ""} onChange={e => setChosenSummary(Number(e.target.value))} disabled={busy}>{meeting.summaries.map(s => <option key={s.id} value={s.id}>{s.template}</option>)}</select></label>
      <div className="team-publication-preview">{summary ? <MdBlock md={summary.content_md} /> : <p>Generate a summary before publishing.</p>}</div>
      <label className="team-checkbox"><input type="checkbox" checked={transcript} disabled={busy} onChange={e => setTranscript(e.target.checked)} />Include the full speaker-labeled transcript ({meeting.segments.length} segments)</label>
      {transcript && <details className="team-transcript"><summary>Review transcript</summary><pre>{reviewedTranscript}</pre></details>}
      <p className="team-muted">My Notes, audio, and video are not included. Summaries can contain information from My Notes, so review the preview. Publishing creates a shared copy; future local edits won’t update it automatically.</p>
      {error && <p className="team-error" role="alert">{error}</p>}<button className="team-primary" onClick={() => void publish()} disabled={busy || !space || !summary}>{busy ? "Publishing…" : "Publish to team"}</button>
    </div>}
  </TeamDialog>;
}
