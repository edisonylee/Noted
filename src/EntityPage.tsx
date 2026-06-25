import { useEffect, useState } from "react";
import { ChevronLeft, Sparkles, ArrowUp } from "lucide-react";
import { api, type EntityProfile, type NoteRow } from "./api";
import { relativeDay } from "./day";
import { colorForType } from "./entityColors";
import { DataView } from "./DataView";

// A dedicated page for ANY entity (person, place, topic, …): header + the full,
// uncapped timeline of mentions. Clicking a mention expands its source note
// inline (from the already-loaded `notes`), so there's no extra round-trip.
export function EntityPage({
  entityId,
  notes,
  onBack,
}: {
  entityId: number;
  notes: NoteRow[];
  onBack: () => void;
}) {
  const [profile, setProfile] = useState<EntityProfile | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [openNoteId, setOpenNoteId] = useState<number | null>(null);
  // Item-scoped ask: answers pinned to this entity (its brain note + captures).
  const [askQ, setAskQ] = useState("");
  const [asking, setAsking] = useState(false);
  const [answer, setAnswer] = useState<string | null>(null);

  useEffect(() => {
    setProfile(null);
    setErr(null);
    setOpenNoteId(null);
    setAskQ("");
    setAnswer(null);
    api.entityProfile(entityId).then(setProfile).catch((e) => setErr(String(e)));
  }, [entityId]);

  async function ask() {
    const q = askQ.trim();
    if (!q || asking) return;
    setAsking(true);
    setAnswer(null);
    try {
      const res = await api.chat(q, [], undefined, entityId);
      setAnswer(res.kind === "answer" ? res.answer : "…");
    } catch (e) {
      setAnswer(`Sorry — ${e}`);
    } finally {
      setAsking(false);
    }
  }

  const span =
    profile?.first_seen && profile.last_seen && profile.first_seen !== profile.last_seen
      ? `${relativeDay(profile.first_seen)} → ${relativeDay(profile.last_seen)}`
      : profile?.last_seen
        ? `Last seen ${relativeDay(profile.last_seen)}`
        : "";

  return (
    <div className="entity-page">
      <button className="ghost-btn entity-back" onClick={onBack}>
        <ChevronLeft size={15} /> Back
      </button>

      {err && <div className="error">{err}</div>}
      {!profile && !err && <p className="muted">loading…</p>}

      {profile && (
        <>
          <header className="entity-page-head">
            <span className="ldot big" style={{ background: colorForType(profile.type) }} />
            <div className="entity-id">
              <h2 className="entity-name">{profile.name}</h2>
              <div className="entity-meta">
                {profile.type}
                {profile.relationship ? ` · ${profile.relationship}` : ""} · {profile.mention_count}{" "}
                {profile.mention_count === 1 ? "mention" : "mentions"}
              </div>
              {span && <div className="entity-meta">{span}</div>}
              {profile.aliases.length > 0 && (
                <div className="person-aliases muted">also: {profile.aliases.join(", ")}</div>
              )}
            </div>
          </header>

          <div className="entity-ask">
            <div className="entity-ask-row">
              <Sparkles size={14} className="entity-ask-mark" />
              <input
                value={askQ}
                placeholder={`Ask about ${profile.name}…`}
                onChange={(e) => setAskQ(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") ask();
                }}
                disabled={asking}
              />
              <button className="send-btn" onClick={ask} disabled={asking || !askQ.trim()}>
                <ArrowUp size={16} strokeWidth={2.5} />
              </button>
            </div>
            {asking && <p className="muted small entity-ask-answer">thinking…</p>}
            {answer && !asking && <p className="entity-ask-answer">{answer}</p>}
          </div>

          <ul className="entity-timeline">
            {profile.mentions.length === 0 && <li className="muted small">No mentions yet.</li>}
            {profile.mentions.map((m, i) => {
              const note = notes.find((n) => n.id === m.note_id);
              const isOpen = openNoteId === m.note_id;
              return (
                <li className="entity-tl-item" key={i}>
                  <button
                    className={"entity-tl-row" + (note ? " has-note" : "")}
                    onClick={() => note && setOpenNoteId(isOpen ? null : m.note_id)}
                    disabled={!note}
                  >
                    <span className="fact-date">{relativeDay(m.date)}</span>
                    <span className="fact-text">{m.text}</span>
                  </button>
                  {isOpen && note && (
                    <div className="entity-tl-note">
                      {note.entries.map((e, j) => (
                        <div className="tl-entry" key={j}>
                          {note.entries.length > 1 && (
                            <div className="tl-entry-cat">{e.category ?? "uncategorized"}</div>
                          )}
                          {e.data && <DataView value={e.data} />}
                        </div>
                      ))}
                      <details className="raw">
                        <summary>raw note</summary>
                        <pre>{note.raw_text}</pre>
                      </details>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        </>
      )}
    </div>
  );
}
