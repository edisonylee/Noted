import { useEffect } from "react";
import {
  AtSign,
  Bookmark,
  BellOff,
  ChevronDown,
  Hash,
  Plus,
  Search,
  SquarePen,
  Users,
  X,
} from "lucide-react";
import { TeamAvatar } from "./TeamAvatar";
import { roomLabel, shortTime } from "./messaging";
import { useNavigationState } from "../useNavigationState";
import type { TeamChatRoom, TeamSnapshot } from "./types";
import "./message-sidebar.css";

export function MessageSidebar({
  data,
  rooms,
  selected,
  inbox,
  saved,
  onSaved,
  drafts,
  filter,
  onFilter,
  onSelect,
  onMentions,
  onCreate,
}: {
  data: TeamSnapshot;
  rooms: TeamChatRoom[];
  selected: string;
  inbox: boolean;
  saved: boolean;
  onSaved: () => void;
  drafts: Record<string, string>;
  filter: string;
  onFilter: (value: string) => void;
  onSelect: (id: string) => void;
  onMentions: () => void;
  onCreate: (kind: "direct" | "channel") => void;
}) {
  const user = data.user.id,
    org = data.org.id;
  const [collapsed, setCollapsed] = useNavigationState<Record<string, boolean>>(
    `team:${org}:${user}:message-groups`,
    { archived: true },
  );
  const selectedRoom = rooms.find((r) => r.id === selected);
  const selectedGroup = !selectedRoom
    ? null
    : selectedRoom.archived_at
      ? "archived"
      : selectedRoom.kind === "direct"
        ? "direct"
        : "channels";
  useEffect(() => {
    if (!inbox && !saved && selectedGroup)
      setCollapsed((old) =>
        old[selectedGroup] ? { ...old, [selectedGroup]: false } : old,
      );
  }, [selected, selectedGroup, inbox, saved]);
  const query = filter.trim().toLocaleLowerCase();
  const matches = (room: TeamChatRoom) =>
    roomLabel(room, user).toLocaleLowerCase().includes(query);
  const directs = rooms
    .filter((r) => r.kind === "direct" && matches(r))
    .sort(
      (a, b) =>
        b.last_activity.localeCompare(a.last_activity) ||
        a.id.localeCompare(b.id),
    );
  const channels = rooms
    .filter((r) => r.kind === "channel" && !r.archived_at && matches(r))
    .sort(
      (a, b) =>
        Number(b.is_default) - Number(a.is_default) ||
        a.name.localeCompare(b.name),
    );
  const archived = rooms
    .filter((r) => r.kind === "channel" && r.archived_at && matches(r))
    .sort((a, b) => a.name.localeCompare(b.name));
  const mentions = rooms.reduce((n, r) => n + (r.unread_mentions ?? 0), 0);
  const badge = (n: number) => (n > 99 ? "99+" : String(n));
  const row = (room: TeamChatRoom) => {
    const title = roomLabel(room, user);
    const peer = room.participants.find((p) => p.id !== user);
    const draft = drafts[room.id]?.trim();
    const last = room.last_message;
    const preview = draft
      ? `Draft: ${draft}`
      : last
        ? `${last.author_id === user ? "You: " : room.kind === "channel" ? `${last.author_name}: ` : ""}${last.body}`
        : room.is_default
          ? "Everyone in your team"
          : room.kind === "direct"
            ? "Start the conversation"
            : "No messages yet";
    const unread = room.unread > 0;
    const mentionCount = room.unread_mentions ?? 0;
    return (
      <button
        key={room.id}
        className={`message-list-row${selected === room.id && !inbox && !saved ? " is-selected" : ""}${unread ? " is-unread" : ""}`}
        aria-current={
          selected === room.id && !inbox && !saved ? "page" : undefined
        }
        title={title}
        onClick={() => onSelect(room.id)}
      >
        <span
          className={`message-list-avatar${room.kind === "channel" ? " is-channel" : ""}`}
          aria-hidden="true"
        >
          {room.kind === "direct" ? (
            <TeamAvatar
              org={org}
              person={
                data.members.find((m) => m.id === peer?.id) ??
                peer ?? { id: "", name: title }
              }
            />
          ) : room.is_default ? (
            <Users size={18} />
          ) : (
            <Hash size={18} />
          )}
        </span>
        <span className="message-list-copy">
          <strong>{title}</strong>
          <span className={`message-list-preview${draft ? " is-draft" : ""}`}>
            {room.notification_mode === "none" && (
              <BellOff size={12} aria-label="Muted" />
            )}
            <span>{preview}</span>
          </span>
        </span>
        <span className="message-list-meta">
          {last && (
            <time dateTime={last.created_at}>{shortTime(last.created_at)}</time>
          )}
          {(unread || mentionCount > 0) && (
            <span
              className="message-list-badge"
              aria-label={`${room.unread} unread messages${mentionCount ? `, ${mentionCount} mentions` : ""}`}
            >
              {mentionCount > 0 ? "@" : ""}
              {unread ? badge(room.unread) : badge(mentionCount)}
            </span>
          )}
        </span>
      </button>
    );
  };
  const group = (
    id: string,
    title: string,
    items: TeamChatRoom[],
    empty: string,
  ) => {
    const expanded = !!query || !collapsed[id];
    const unread = items.reduce((n, r) => n + r.unread, 0);
    return (
      <section className="message-list-group" aria-label={title} key={id}>
        <div className="message-list-group-head">
          <button
            className="message-list-disclosure"
            aria-expanded={expanded}
            aria-controls={`message-group-${org}-${id}`}
            onClick={() => setCollapsed((old) => ({ ...old, [id]: !old[id] }))}
            disabled={!!query}
          >
            <ChevronDown size={13} className={expanded ? "" : "is-closed"} />
            <span>{title}</span>
            {!expanded && unread > 0 && (
              <span
                className="message-group-unread"
                aria-label={`${unread} unread messages`}
              >
                {badge(unread)}
              </span>
            )}
          </button>
          {id === "channels" && (
            <button
              className="message-list-icon"
              aria-label="Create channel"
              title="Create channel"
              onClick={() => onCreate("channel")}
            >
              <Plus size={16} />
            </button>
          )}
        </div>
        <div id={`message-group-${org}-${id}`} hidden={!expanded}>
          {items.map(row)}
          {!items.length && <p className="message-list-empty">{empty}</p>}
        </div>
      </section>
    );
  };
  return (
    <aside className="message-sidebar" aria-label="Team conversations">
      <header className="message-sidebar-head">
        <div className="message-sidebar-title">
          <h2>Messages</h2>
          <button
            className="message-list-icon"
            aria-label="New message"
            title="New message"
            onClick={() => onCreate("direct")}
          >
            <SquarePen size={18} />
          </button>
        </div>
        <label className="message-list-filter">
          <Search size={14} aria-hidden="true" />
          <input
            type="search"
            aria-label="Filter conversations"
            placeholder="Filter conversations"
            value={filter}
            onChange={(e) => onFilter(e.target.value)}
          />
          {filter && (
            <button
              aria-label="Clear conversation filter"
              onClick={() => onFilter("")}
            >
              <X size={13} />
            </button>
          )}
        </label>
        <button
          className={`message-mentions-link${inbox ? " is-selected" : ""}`}
          aria-current={inbox ? "page" : undefined}
          onClick={onMentions}
        >
          <AtSign size={17} />
          <span>Mentions</span>
          {mentions > 0 && (
            <span
              className="message-list-badge"
              aria-label={`${mentions} unread mentions`}
            >
              {badge(mentions)}
            </span>
          )}
        </button>
        {rooms.some((r) => r.saved_messages_enabled) && (
          <button
            className={`message-mentions-link${saved ? " is-selected" : ""}`}
            aria-current={saved ? "page" : undefined}
            onClick={onSaved}
          >
            <Bookmark size={17} />
            <span>Saved messages</span>
          </button>
        )}
      </header>
      <div className="message-sidebar-scroll">
        {query && !directs.length && !channels.length && !archived.length ? (
          <div className="message-list-no-results">
            <p>No conversations found</p>
            <span>Try another name.</span>
            <button onClick={() => onFilter("")}>Clear filter</button>
          </div>
        ) : (
          <>
            {(!query || directs.length > 0) &&
              group(
                "direct",
                "Direct messages",
                directs,
                "Use New message to start a conversation.",
              )}
            {(!query || channels.length > 0) &&
              group("channels", "Channels", channels, "No channels yet.")}
            {archived.length > 0 && group("archived", "Archived", archived, "")}
          </>
        )}
      </div>
    </aside>
  );
}
