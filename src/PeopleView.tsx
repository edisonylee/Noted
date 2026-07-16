import { useEffect, useState } from "react";
import { Check, GitMerge, Loader2, Pencil, Users, X } from "lucide-react";
import { api, type MergeSuggestion, type PersonProfile } from "./api";
import { relativeDay } from "./day";

export function PeopleView({ onOpenPerson }: { onOpenPerson?: (id: number) => void }) {
  const [people, setPeople] = useState<PersonProfile[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [backfilling, setBackfilling] = useState(false);
  const [backfillMsg, setBackfillMsg] = useState<string | null>(null);
  // Likely-duplicate pairs from the retro embedding scan; merging or dismissing
  // each one is how the catalog cleans itself up over time.
  const [suggestions, setSuggestions] = useState<MergeSuggestion[]>([]);

  const refresh = () =>
    api
      .listPeople()
      .then(setPeople)
      .catch((e) => setErr(String(e)))
      .finally(() => setLoading(false));
  const loadSuggestions = () => api.suggestEntityMerges().then(setSuggestions).catch(() => {});

  useEffect(() => {
    refresh();
    loadSuggestions();
  }, []);

  // Naming flow: confirm the AI-suggested display name, dismiss it, or type a
  // name directly. Confirm renames the entity and keeps the email as an alias.
  const [renaming, setRenaming] = useState<number | null>(null);
  const [renameText, setRenameText] = useState("");

  async function confirmName(p: PersonProfile, name: string) {
    setErr(null);
    try {
      await api.confirmPersonName(p.id, name);
      setRenaming(null);
      await refresh();
    } catch (e) {
      setErr(String(e));
    }
  }

  async function dismissName(p: PersonProfile) {
    try {
      await api.dismissPersonName(p.id);
      setPeople((all) => all.map((o) => (o.id === p.id ? { ...o, suggested_name: null } : o)));
    } catch (e) {
      setErr(String(e));
    }
  }

  async function mergeInto(dropId: number, keepId: number) {
    if (!keepId || keepId === dropId) return;
    setErr(null);
    try {
      await api.mergeEntities(keepId, dropId);
      await refresh();
      await loadSuggestions();
    } catch (e) {
      setErr(String(e));
    }
  }

  // Accept a suggestion: keep the better-established side. (The dropdown on a
  // card still covers merging the other direction.)
  async function acceptSuggestion(s: MergeSuggestion) {
    const [keep, drop] = s.a_mentions >= s.b_mentions ? [s.a_id, s.b_id] : [s.b_id, s.a_id];
    setErr(null);
    try {
      await api.mergeEntities(keep, drop);
      await refresh();
      await loadSuggestions();
    } catch (e) {
      setErr(String(e));
    }
  }

  async function dismissSuggestion(s: MergeSuggestion) {
    setErr(null);
    try {
      await api.dismissMergeSuggestion(s.a_id, s.b_id);
      setSuggestions((all) => all.filter((o) => !(o.a_id === s.a_id && o.b_id === s.b_id)));
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

  // People you've mentioned once ride in a collapsed drawer below the grid so
  // the main view is the people who actually recur. If *everyone* is a one-off
  // (young graph), the split would just hide the whole view — show them all.
  const recurring = people.filter((p) => p.mention_count >= 2);
  const oneOffs = recurring.length > 0 ? people.filter((p) => p.mention_count < 2) : [];
  const mainList = recurring.length > 0 ? recurring : people;

  const personCard = (p: PersonProfile) => (
          <div
            className="entry-card person-card clickable"
            key={p.id}
            onClick={() => onOpenPerson?.(p.id)}
            role={onOpenPerson ? "button" : undefined}
          >
            <div className="person-head">
              {renaming === p.id ? (
                <form
                  className="person-rename"
                  onClick={(e) => e.stopPropagation()}
                  onSubmit={(e) => {
                    e.preventDefault();
                    if (renameText.trim()) confirmName(p, renameText.trim());
                  }}
                >
                  <input
                    autoFocus
                    value={renameText}
                    onChange={(e) => setRenameText(e.target.value)}
                    placeholder="Display name"
                  />
                  <button type="submit" className="icon-btn" title="Save name">
                    <Check size={13} />
                  </button>
                  <button
                    type="button"
                    className="icon-btn"
                    title="Cancel"
                    onClick={() => setRenaming(null)}
                  >
                    <X size={13} />
                  </button>
                </form>
              ) : (
                <>
                  <h3 className="person-name">{p.name}</h3>
                  <button
                    className="icon-btn person-rename-btn"
                    title="Rename this person"
                    onClick={(e) => {
                      e.stopPropagation();
                      setRenameText(p.suggested_name ?? (p.name.includes("@") ? "" : p.name));
                      setRenaming(p.id);
                    }}
                  >
                    <Pencil size={12} />
                  </button>
                </>
              )}
              {p.relationship && <span className="badge routed">{p.relationship}</span>}
              <span className="badge existing">
                {p.mention_count} {p.mention_count === 1 ? "mention" : "mentions"}
              </span>
            </div>

            {p.suggested_name && renaming !== p.id && (
              <div className="name-suggest" onClick={(e) => e.stopPropagation()}>
                Call them <strong>{p.suggested_name}</strong>?
                <button className="link" onClick={() => confirmName(p, p.suggested_name!)}>
                  confirm
                </button>
                <button className="link" onClick={() => dismissName(p)}>
                  dismiss
                </button>
              </div>
            )}

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
              {p.mentions.slice(0, 4).map((m, i) => (
                <li key={i}>
                  <span className="fact-date">{relativeDay(m.date)}</span>
                  <span className="fact-text">{m.text}</span>
                </li>
              ))}
              {p.mentions.length > 4 && (
                <li className="muted small">+{p.mentions.length - 4} more — open for the full story</li>
              )}
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

      {suggestions.length > 0 && (
        <div className="merge-suggest">
          <div className="merge-suggest-label">
            <GitMerge size={13} /> These might be the same — merge or dismiss
          </div>
          {suggestions.map((s) => {
            const aKeeps = s.a_mentions >= s.b_mentions;
            return (
              <div className="merge-suggest-row" key={`${s.a_id}-${s.b_id}`}>
                <span className="ms-names">
                  <strong>{aKeeps ? s.b_name : s.a_name}</strong>
                  {" → "}
                  <strong>{aKeeps ? s.a_name : s.b_name}</strong>
                  <span className="muted">
                    {" "}
                    {s.etype} · {Math.round(s.similarity * 100)}% match
                  </span>
                </span>
                <span className="ms-actions">
                  <button className="link" onClick={() => acceptSuggestion(s)}>
                    merge
                  </button>
                  <button className="link" onClick={() => dismissSuggestion(s)}>
                    not the same
                  </button>
                </span>
              </div>
            );
          })}
        </div>
      )}

      {!loading && people.length === 0 && (
        <div className="people-empty">
          <p className="muted">
            No people yet — mention someone by name in a note (&ldquo;coffee with Sarah, she just got engaged&rdquo;)
            and they&rsquo;ll show up here.
          </p>
          {rebuildBtn}
        </div>
      )}

      <div className="people-grid">{mainList.map(personCard)}</div>

      {oneOffs.length > 0 && (
        <details className="people-oneoffs">
          <summary>
            {oneOffs.length} {oneOffs.length === 1 ? "person" : "people"} mentioned once
          </summary>
          <div className="people-grid">{oneOffs.map(personCard)}</div>
        </details>
      )}
    </div>
  );
}
