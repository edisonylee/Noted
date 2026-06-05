import { useEffect, useState } from "react";
import { Loader2, Users } from "lucide-react";
import { api, type PersonProfile } from "./api";
import { relativeDay } from "./day";

export function PeopleView({ onOpenPerson }: { onOpenPerson?: (id: number) => void }) {
  const [people, setPeople] = useState<PersonProfile[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [backfilling, setBackfilling] = useState(false);
  const [backfillMsg, setBackfillMsg] = useState<string | null>(null);

  const refresh = () =>
    api
      .listPeople()
      .then(setPeople)
      .catch((e) => setErr(String(e)))
      .finally(() => setLoading(false));

  useEffect(() => {
    refresh();
  }, []);

  async function mergeInto(dropId: number, keepId: number) {
    if (!keepId || keepId === dropId) return;
    setErr(null);
    try {
      await api.mergeEntities(keepId, dropId);
      await refresh();
    } catch (e) {
      setErr(String(e));
    }
  }

  // Re-derive people from past notes (for names the model filed into a field but
  // never surfaced as an entity). Idempotent on the backend, so safe to re-run.
  async function rebuild() {
    if (backfilling) return;
    setBackfilling(true);
    setBackfillMsg(null);
    try {
      const added = await api.backfillEntities();
      await refresh();
      setBackfillMsg(
        added > 0 ? `Found ${added} new mention${added === 1 ? "" : "s"} from past notes.` : "No new people found in past notes."
      );
    } catch (e) {
      setBackfillMsg(String(e));
    } finally {
      setBackfilling(false);
    }
  }

  const rebuildBtn = (
    <button className="ghost-btn" onClick={rebuild} disabled={backfilling}>
      {backfilling ? <Loader2 size={14} className="spin" /> : <Users size={14} />}
      Rebuild people from past notes
    </button>
  );

  return (
    <div className="people">
      <div className="people-head">
        <p className="muted">
          Everyone you mention is gathered here — what they&rsquo;re up to, how you know them, and when you last
          crossed paths.
        </p>
        {people.length > 0 && rebuildBtn}
      </div>
      {backfillMsg && <div className="muted small">{backfillMsg}</div>}

      {err && <div className="error">{err}</div>}

      {!loading && people.length === 0 && (
        <div className="people-empty">
          <p className="muted">
            No people yet — mention someone by name in a note (&ldquo;coffee with Sarah, she just got engaged&rdquo;)
            and they&rsquo;ll show up here.
          </p>
          {rebuildBtn}
        </div>
      )}

      <div className="people-grid">
        {people.map((p) => (
          <div
            className="entry-card person-card clickable"
            key={p.id}
            onClick={() => onOpenPerson?.(p.id)}
            role={onOpenPerson ? "button" : undefined}
          >
            <div className="person-head">
              <h3 className="person-name">{p.name}</h3>
              {p.relationship && <span className="badge routed">{p.relationship}</span>}
              <span className="badge existing">
                {p.mention_count} {p.mention_count === 1 ? "mention" : "mentions"}
              </span>
            </div>

            {p.last_seen && (
              <div className="person-meta">
                {p.first_seen && p.first_seen !== p.last_seen
                  ? `${relativeDay(p.first_seen)} → ${relativeDay(p.last_seen)}`
                  : `Last seen ${relativeDay(p.last_seen)}`}
              </div>
            )}

            {p.aliases.length > 0 && (
              <div className="person-aliases muted">also: {p.aliases.join(", ")}</div>
            )}

            <ul className="person-facts">
              {p.mentions.map((m, i) => (
                <li key={i}>
                  <span className="fact-date">{relativeDay(m.date)}</span>
                  <span className="fact-text">{m.text}</span>
                </li>
              ))}
            </ul>

            {people.length > 1 && (
              <div className="person-merge" onClick={(e) => e.stopPropagation()}>
                <label>
                  <Users size={12} /> same as
                </label>
                <select
                  value=""
                  onChange={(e) => mergeInto(p.id, Number(e.target.value))}
                  title="Merge this person into another (combines their mentions)"
                >
                  <option value="">merge into…</option>
                  {people
                    .filter((o) => o.id !== p.id)
                    .map((o) => (
                      <option key={o.id} value={o.id}>
                        {o.name}
                      </option>
                    ))}
                </select>
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
