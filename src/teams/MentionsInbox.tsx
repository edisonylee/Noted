import { useNavigationState } from "../useNavigationState";
import { useEffect, useRef, useState } from "react";
import { AtSign, Loader, RefreshCw } from "lucide-react";
import { team, orgPath } from "./client";
import type { TeamMentionPage } from "./types";
import { roomLabel } from "./messaging";

export function MentionsInbox({ org, user, onOpen }: { org: string; user: string; onOpen: (id: string) => void }) {
  const [unread, setUnread] = useNavigationState(`team:${org}:${user}:mentions-unread`, false);
  const [page, setPage] = useState<TeamMentionPage>({ items: [], next_before: null });
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  const generation = useRef(0);
  useEffect(() => {
    let alive = true;
    ++generation.current;
    setMoreBusy(false);
    setPage({ items: [], next_before: null });
    setBusy(true);
    const load = async () => {
      try {
        const result = await team.request<TeamMentionPage>("GET", orgPath(org, `/mentions?unread=${unread}`));
        if (!alive) return;
        setPage(result); setError("");
      } catch (e) { if (alive) { setPage({ items: [], next_before: null }); setError(String(e)); } }
      finally { if (alive) { setBusy(false);  } }
    };
    void load();
    const refresh = () => setRetry((n) => n + 1);
    window.addEventListener("focus", refresh);
    return () => { alive = false; ++generation.current; window.removeEventListener("focus", refresh); };
  }, [org, unread, retry]);
  const [moreBusy, setMoreBusy] = useState(false);

  return <section className="mentions-inbox" aria-label="Mentions inbox">
    <header className="messages-room-head"><div><h1><AtSign size={20} /> Mentions</h1><p>Messages where teammates mentioned you, including thread replies.</p></div>
      <button className="team-text-button" onClick={() => setRetry((n) => n + 1)} aria-label="Refresh mentions"><RefreshCw size={16} /></button>
    </header>
    <label className="mentions-filter"><input type="checkbox" checked={unread} onChange={(e) => setUnread(e.target.checked)} /> Unread only</label>
    {error && <p className="team-error" role="alert">{error}</p>}
    <div className="mentions-list">
      {page.items.map((item) => <button key={item.message.id} className="mention-inbox-row" onClick={() => onOpen(item.message.id)}>
        <span><strong>{item.message.author_name}</strong><time>{new Date(item.message.created_at).toLocaleString()}</time></span>
        <small>{roomLabel(item.room, user)}{item.parent ? " · Thread reply" : ""}{item.unread ? " · Unread" : ""}</small>
        <p>{item.message.body}</p>
      </button>)}
      {busy ? <p className="messages-empty"><Loader size={16} className="spin" /> Loading mentions…</p> : !error && !page.items.length && <p className="messages-empty">{unread ? "You’re caught up. No unread mentions." : "Your mentions will appear here."}</p>}
      {page.next_before != null && <button className="team-text-button" disabled={moreBusy} onClick={async () => {
        const epoch = generation.current;
        setMoreBusy(true);
        try {
          const next = await team.request<TeamMentionPage>("GET", orgPath(org, `/mentions?unread=${unread}&before=${page.next_before}`));
          if (epoch !== generation.current) return;
          setPage((old) => ({ items: [...new Map([...old.items, ...next.items].map((item) => [item.message.id, item])).values()], next_before: next.next_before }));
        } catch (e) { if (epoch === generation.current) setError(String(e)); }
        finally { if (epoch === generation.current) setMoreBusy(false); }
      }}>{moreBusy ? "Loading…" : "Load older mentions"}</button>}
    </div>
  </section>;
}
