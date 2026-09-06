import { useEffect, useRef, useState } from "react";
import {
  Search,
  Loader,
  MessageSquare,
  BookOpen,
  ChevronDown,
  SlidersHorizontal,
  RotateCcw,
} from "lucide-react";
import { useNavigationState } from "../useNavigationState";
import { team, orgPath } from "./client";
import type {
  TeamChatRoom,
  TeamSearchHit,
  TeamSearchPage,
  TeamSnapshot,
} from "./types";
import "./search.css";

export function TeamSearch({
  data,
  room,
  onRoom,
  onOpen,
}: {
  data: TeamSnapshot;
  room: string;
  onRoom: (id: string) => void;
  onOpen: (hit: TeamSearchHit) => void;
}) {
  const org = data.org.id;
  const scope = `team:${org}:${data.user.id}:search`;
  const [query, setQuery] = useNavigationState(`${scope}:query`, "");
  const [kind, setKind] = useNavigationState(`${scope}:kind`, "all");
  const [author, setAuthor] = useNavigationState(`${scope}:author`, "");
  const [since, setSince] = useNavigationState(`${scope}:since`, "");
  const [until, setUntil] = useNavigationState(`${scope}:until`, "");
  const [rooms, setRooms] = useState<TeamChatRoom[]>([]);
  const [result, setResult] = useState<TeamSearchPage | null>(null);
  const [resultKey, setResultKey] = useState("");
  const [resultAccess, setResultAccess] = useState<number | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [loadingGroup, setLoadingGroup] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const generation = useRef(0);
  const effectiveKind = room ? "messages" : kind;
  const params = new URLSearchParams({
    q: query,
    kind: effectiveKind,
    room,
    author,
    since,
    until,
    limit: effectiveKind === "all" ? "5" : "20",
  });
  const key = params.toString();
  const latestKey = useRef(key);
  latestKey.current = key;
  useEffect(() => {
    let active = true;
    team
      .request<TeamChatRoom[]>("GET", orgPath(org, "/chat-rooms"))
      .then((rows) => {
        if (active) setRooms(rows);
      })
      .catch(() => {
        if (active) setRooms([]);
      });
    return () => {
      active = false;
    };
  }, [org, data.access_version, refresh]);
  useEffect(() => {
    const epoch = ++generation.current;
    setResult(null);
    setError("");
    setLoadingGroup(null);
    setBusy(!!query.trim());
    if (!query.trim()) return;
    const timer = window.setTimeout(async () => {
      try {
        const page = await team.request<TeamSearchPage>(
          "GET",
          orgPath(org, `/search?${key}`),
        );
        if (epoch !== generation.current || key !== latestKey.current) return;
        setResult(page);
        setResultKey(key);
        setResultAccess(data.access_version);
      } catch (e) {
        if (epoch === generation.current) setError(String(e));
      } finally {
        if (epoch === generation.current) setBusy(false);
      }
    }, 250);
    return () => {
      clearTimeout(timer);
      ++generation.current;
    };
  }, [org, key, refresh, data.access_version]);
  useEffect(() => {
    const wake = () => {
      if (document.visibilityState === "visible") {
        ++generation.current;
        setResult(null);
        setRefresh((n) => n + 1);
      }
    };
    const timer = window.setInterval(wake, 30_000);
    window.addEventListener("focus", wake);
    document.addEventListener("visibilitychange", wake);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", wake);
      document.removeEventListener("visibilitychange", wake);
    };
  }, []);
  const more = async (group: "messages" | "meetings") => {
    if (!result || !result[group].cursor || loadingGroup || busy) return;
    const epoch = generation.current;
    setLoadingGroup(group);
    try {
      const next = new URLSearchParams(key);
      next.set("kind", group);
      next.set(`${group}_cursor`, result[group].cursor!);
      const page = await team.request<TeamSearchPage>(
        "GET",
        orgPath(org, `/search?${next}`),
      );
      if (epoch !== generation.current || key !== latestKey.current) return;
      setResult((old) => {
        if (!old) return null;
        // Ranked offset pages may shift after edits: never show duplicate rows.
        const merge = <T extends { id: string }>(a: T[], b: T[]) => [
          ...a,
          ...b.filter((hit) => !a.some((item) => item.id === hit.id)),
        ];
        return group === "messages"
          ? {
              ...old,
              messages: {
                hits: merge(old.messages.hits, page.messages.hits),
                cursor: page.messages.cursor,
              },
            }
          : {
              ...old,
              meetings: {
                hits: merge(old.meetings.hits, page.meetings.hits),
                cursor: page.meetings.cursor,
              },
            };
      });
    } catch (e) {
      if (epoch === generation.current) {
        setResult(null);
        setError(String(e));
      }
    } finally {
      if (epoch === generation.current) setLoadingGroup(null);
    }
  };
  const visible =
    resultKey === key && resultAccess === data.access_version ? result : null;
  const row = (hit: TeamSearchHit) => (
    <button
      className="team-search-result"
      key={hit.id}
      onClick={() => onOpen(hit)}
    >
      <span className="team-search-result-icon">
        {hit.kind === "message" ? (
          <MessageSquare size={17} />
        ) : (
          <BookOpen size={17} />
        )}
      </span>
      <span className="team-search-result-content">
        <span className="team-search-result-meta">
          <strong>
            {hit.kind === "message" ? hit.author_name : hit.title}
          </strong>
          {hit.kind === "message" && <span>{hit.room_label}</span>}
          <time
            dateTime={hit.kind === "message" ? hit.created_at : hit.occurred_at}
          >
            {new Date(
              hit.kind === "message" ? hit.created_at : hit.occurred_at,
            ).toLocaleDateString()}
          </time>
        </span>
        <span className="team-search-snippet">
          {hit.snippet.map((part, i) =>
            part.match ? (
              <mark key={i}>{part.text}</mark>
            ) : (
              <span key={i}>{part.text}</span>
            ),
          )}
        </span>
      </span>
    </button>
  );
  const activeFilterCount = [
    effectiveKind !== "all",
    !!room,
    !!author,
    !!since,
    !!until,
  ].filter(Boolean).length;
  return (
    <main className="team-search">
      <header>
        <h1>Search</h1>
        <p>Find messages and published meetings you have access to.</p>
      </header>
      <label className="team-search-query">
        <Search size={19} />
        <input
          autoFocus
          type="search"
          aria-label="Search messages and meetings"
          placeholder="Search keywords, names, or a topic"
          maxLength={256}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </label>
      <p className="team-search-hint">
        Use keywords such as “pricing” or “Q4 budget”. All words must match.
      </p>
      <details className="team-search-filter-panel">
        <summary>
          <SlidersHorizontal size={14} aria-hidden="true" />
          Filters
          {activeFilterCount > 0 && <span>{activeFilterCount} active</span>}
          <ChevronDown
            size={14}
            className="team-filter-disclosure"
            aria-hidden="true"
          />
        </summary>
        <div className="team-search-filters">
          <label>
            Search in
            <span className="team-filter-select">
              <select
                value={effectiveKind}
                disabled={!!room}
                onChange={(e) => setKind(e.target.value)}
              >
                <option value="all">All content</option>
                <option value="messages">Messages</option>
                <option value="meetings">Meetings</option>
              </select>
              <ChevronDown size={14} aria-hidden="true" />
            </span>
          </label>
          <label>
            Conversation
            <span className="team-filter-select">
              <select value={room} onChange={(e) => onRoom(e.target.value)}>
                <option value="">All conversations</option>
                {room && !rooms.some((r) => r.id === room) && (
                  <option value={room}>Selected conversation</option>
                )}
                {rooms.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.kind === "channel"
                      ? r.is_default
                        ? "Team chat"
                        : `#${r.name}`
                      : r.participants
                          .filter((p) => p.id !== data.user.id)
                          .map((p) => p.name)
                          .join(", ")}
                    {r.archived_at ? " (archived)" : ""}
                  </option>
                ))}
              </select>
              <ChevronDown size={14} aria-hidden="true" />
            </span>
          </label>
          <label>
            Sender / publisher
            <span className="team-filter-select">
              <select
                value={author}
                onChange={(e) => setAuthor(e.target.value)}
              >
                <option value="">Anyone</option>
                {data.members.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name}
                  </option>
                ))}
              </select>
              <ChevronDown size={14} aria-hidden="true" />
            </span>
          </label>
          <label>
            From
            <input
              type="date"
              value={since}
              onChange={(e) => setSince(e.target.value)}
            />
          </label>
          <label>
            Through
            <input
              type="date"
              value={until}
              onChange={(e) => setUntil(e.target.value)}
            />
          </label>
          <button
            className="team-text-button team-filter-reset"
            disabled={activeFilterCount === 0}
            onClick={() => {
              setKind("all");
              onRoom("");
              setAuthor("");
              setSince("");
              setUntil("");
            }}
          >
            <RotateCcw size={13} aria-hidden="true" />
            Clear filters
          </button>
        </div>
      </details>
      {busy && (
        <p role="status">
          <Loader size={16} className="spin" /> Searching…
        </p>
      )}
      {error && (
        <div role="alert" className="team-error">
          Search unavailable: {error}{" "}
          <button
            className="team-text-button"
            onClick={() => setRefresh((n) => n + 1)}
          >
            Retry
          </button>
        </div>
      )}
      {!query.trim() && (
        <p className="team-search-empty">
          Enter a few keywords to find a conversation or meeting.
        </p>
      )}
      {visible && !busy && (
        <>
          {effectiveKind !== "meetings" && (
            <section aria-label="Message results">
              <h2>Messages</h2>
              {visible.messages.hits.map(row)}
              {!visible.messages.hits.length && <p>No matching messages.</p>}
              {visible.messages.cursor && (
                <button
                  className="team-text-button"
                  disabled={!!loadingGroup}
                  onClick={() => void more("messages")}
                >
                  {loadingGroup === "messages" ? "Loading…" : "More messages"}
                </button>
              )}
            </section>
          )}
          {effectiveKind !== "messages" && (
            <section aria-label="Meeting results">
              <h2>Meetings</h2>
              {visible.meetings.hits.map(row)}
              {!visible.meetings.hits.length && <p>No matching meetings.</p>}
              {visible.meetings.cursor && (
                <button
                  className="team-text-button"
                  disabled={!!loadingGroup}
                  onClick={() => void more("meetings")}
                >
                  {loadingGroup === "meetings" ? "Loading…" : "More meetings"}
                </button>
              )}
            </section>
          )}
        </>
      )}
    </main>
  );
}
