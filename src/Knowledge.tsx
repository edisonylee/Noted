import { useEffect, useMemo, useState } from "react";
import { Users, Network, Briefcase, Search } from "lucide-react";
import { PeopleView } from "./PeopleView";
import { SelfView } from "./Self";
import { WorkView } from "./Work";
import { EntityPage } from "./EntityPage";
import { api, type EntityRow, type NoteRow, type PersonProfile } from "./api";
import { colorForType } from "./entityColors";

// "Knowledge" groups the "what noted knows" surfaces — People, the Self entity
// graph, and the Work lens over imported brain vaults — behind one destination
// with a section toggle, plus a search bar and a per-entity page reachable from
// anywhere.
type Section = "people" | "self" | "work";

const TYPES = ["person", "place", "activity", "food", "item", "org", "topic", "project", "decision", "reference", "doc"];

export function KnowledgeView({ theme, notes }: { theme: string; notes: NoteRow[] }) {
  const [section, setSection] = useState<Section>("people");
  const [selectedEntityId, setSelectedEntityId] = useState<number | null>(null);

  // Search/filter state + the data it filters (loaded once, lazily on first use).
  const [query, setQuery] = useState("");
  const [typeFilter, setTypeFilter] = useState<string>("all");
  const [relFilter, setRelFilter] = useState<string>("all");
  const [people, setPeople] = useState<PersonProfile[]>([]);
  const [entities, setEntities] = useState<EntityRow[]>([]);

  const searching = query.trim() !== "" || typeFilter !== "all" || relFilter !== "all";

  useEffect(() => {
    api.listPeople().then(setPeople).catch(() => {});
    api.listEntities().then(setEntities).catch(() => {});
  }, []);

  // Distinct relationships present, for the relationship dropdown.
  const relationships = useMemo(
    () => Array.from(new Set(people.map((p) => p.relationship).filter(Boolean) as string[])).sort(),
    [people]
  );

  // Per-entity page takes over the whole view when an entity is selected.
  if (selectedEntityId != null) {
    return (
      <div className="knowledge">
        <EntityPage entityId={selectedEntityId} notes={notes} onBack={() => setSelectedEntityId(null)} />
      </div>
    );
  }

  // Unified, client-side search results across people + all entities.
  const q = query.trim().toLowerCase();
  const peopleById = new Map(people.map((p) => [p.id, p]));
  const results = entities
    .filter((e) => (typeFilter === "all" ? true : e.type === typeFilter))
    .filter((e) => {
      if (relFilter !== "all") {
        const p = peopleById.get(e.id);
        if (!p || p.relationship !== relFilter) return false;
      }
      return true;
    })
    .filter((e) => {
      if (!q) return true;
      const p = peopleById.get(e.id);
      const hay = [e.name, ...(p?.aliases ?? []), p?.relationship ?? ""].join(" ").toLowerCase();
      return hay.includes(q);
    })
    .sort((a, b) => b.mention_count - a.mention_count);

  return (
    <div className="knowledge">
      <div className="kn-search">
        <Search size={15} className="kn-search-icon" />
        <input
          placeholder="Search people, places, topics…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          spellCheck={false}
        />
        <select value={typeFilter} onChange={(e) => setTypeFilter(e.target.value)}>
          <option value="all">all types</option>
          {TYPES.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
        {relationships.length > 0 && (
          <select value={relFilter} onChange={(e) => setRelFilter(e.target.value)}>
            <option value="all">any relationship</option>
            {relationships.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </select>
        )}
      </div>

      {searching ? (
        <div className="kn-results">
          {results.length === 0 && <p className="muted">No matches.</p>}
          {results.map((e) => (
            <button className="kn-result" key={e.id} onClick={() => setSelectedEntityId(e.id)}>
              <span className="ldot" style={{ background: colorForType(e.type) }} />
              <span className="kn-result-name">{e.name}</span>
              <span className="kn-result-type">{e.type}</span>
              <span className="kn-result-count">
                {e.mention_count} {e.mention_count === 1 ? "mention" : "mentions"}
              </span>
            </button>
          ))}
        </div>
      ) : (
        <>
          <div className="kn-tabs">
            <button
              className={"kn-tab" + (section === "people" ? " on" : "")}
              onClick={() => setSection("people")}
            >
              <Users size={15} /> People
            </button>
            <button
              className={"kn-tab" + (section === "self" ? " on" : "")}
              onClick={() => setSection("self")}
            >
              <Network size={15} /> Self
            </button>
            <button
              className={"kn-tab" + (section === "work" ? " on" : "")}
              onClick={() => setSection("work")}
            >
              <Briefcase size={15} /> Work
            </button>
          </div>
          {section === "people" ? (
            <PeopleView onOpenPerson={setSelectedEntityId} />
          ) : section === "self" ? (
            <SelfView theme={theme} onOpenEntity={setSelectedEntityId} />
          ) : (
            <WorkView theme={theme} onOpenEntity={setSelectedEntityId} />
          )}
        </>
      )}
    </div>
  );
}
