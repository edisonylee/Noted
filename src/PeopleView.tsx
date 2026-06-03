import { useEffect, useState } from "react";
import { Users } from "lucide-react";
import { api, type PersonProfile } from "./api";

// "Today" / "Yesterday" / "Jun 1" (year only if not the current year).
function relativeDay(dateStr: string): string {
  const d = new Date(dateStr + "T00:00:00");
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const diff = Math.round((today.getTime() - d.getTime()) / 86_400_000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  const sameYear = d.getFullYear() === today.getFullYear();
  return d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
}

export function PeopleView() {
  const [people, setPeople] = useState<PersonProfile[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

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

  return (
    <div className="people">
      <div className="people-head">
        <p className="muted">
          Everyone you mention is gathered here — what they&rsquo;re up to, how you know them, and when you last
          crossed paths.
        </p>
      </div>

      {err && <div className="error">{err}</div>}

      {!loading && people.length === 0 && (
        <p className="muted">
          No people yet — mention someone by name in a note (&ldquo;coffee with Sarah, she just got engaged&rdquo;)
          and they&rsquo;ll show up here.
        </p>
      )}

      <div className="people-grid">
        {people.map((p) => (
          <div className="entry-card person-card" key={p.id}>
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
              <div className="person-merge">
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
