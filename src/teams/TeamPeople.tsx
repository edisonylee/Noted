import { MessageSquare, Settings, Users } from "lucide-react";
import { useState } from "react";
import type { TeamSnapshot } from "./types";
import { initials } from "./presentation";

export function TeamPeople({
  data,
  onMessage,
  onManage,
}: {
  data: TeamSnapshot;
  onMessage: (member: string) => Promise<void>;
  onManage: () => void;
}) {
  const [query, setQuery] = useState("");
  const [pending, setPending] = useState("");
  const [error, setError] = useState("");
  const members = data.members.filter((person) =>
    person.name.toLowerCase().includes(query.trim().toLowerCase()),
  );
  return (
    <main className="team-people-page">
      <header className="team-library-head">
        <div>
          <span className="team-eyebrow">{data.org.name}</span>
          <h1>People</h1>
          <p>Find a teammate and start a private conversation.</p>
        </div>
        <button className="team-text-button" onClick={onManage}>
          <Settings size={15} />
          {data.org.role === "member" ? "Team settings" : "Invite & manage"}
        </button>
      </header>
      <label className="team-people-search">
        <Users size={16} />
        <input
          type="search"
          aria-label="Find a teammate"
          placeholder="Find a teammate"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </label>
      {error && (
        <p className="team-error" role="alert">
          {error}
        </p>
      )}
      <div className="team-people-list">
        {members.map((person) => (
          <div className="team-person-row" key={person.id}>
            <span className="team-person-avatar" aria-hidden="true">
              {initials(person.name)}
            </span>
            <div>
              <strong>
                {person.name}
                {person.id === data.user.id && <small> you</small>}
              </strong>
              <p>
                {person.role === "owner"
                  ? "Team owner"
                  : person.role === "admin"
                    ? "Admin"
                    : "Member"}
              </p>
            </div>
            {person.id !== data.user.id && (
              <button
                className="team-text-button"
                disabled={!!pending}
                aria-label={`Message ${person.name}`}
                onClick={async () => {
                  if (pending) return;
                  setPending(person.id);
                  setError("");
                  try {
                    await onMessage(person.id);
                  } catch (error) {
                    setError(String(error));
                  } finally {
                    setPending("");
                  }
                }}
              >
                <MessageSquare size={15} />
                {pending === person.id ? "Opening…" : "Message"}
              </button>
            )}
          </div>
        ))}
      </div>
      {!members.length && (
        <p className="team-empty">No teammates match that name.</p>
      )}
    </main>
  );
}
